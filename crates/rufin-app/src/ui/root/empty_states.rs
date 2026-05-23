impl Shell {
    fn placeholder_view(&self, title: &str, body: &str) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("empty-state");
        wrapper.set_vexpand(true);
        wrapper.set_hexpand(true);
        wrapper.set_valign(gtk::Align::Center);
        wrapper.set_halign(gtk::Align::Center);

        let heading = gtk::Label::new(Some(&tr(title)));
        heading.add_css_class("section-heading");
        let label = gtk::Label::new(Some(&tr(body)));
        label.add_css_class("muted");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        wrapper.append(&heading);
        wrapper.append(&label);
        wrapper.upcast()
    }
    fn route_empty_view(&self, body: &str) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("empty-state");
        wrapper.set_vexpand(true);
        wrapper.set_hexpand(true);
        wrapper.set_valign(gtk::Align::Center);
        wrapper.set_halign(gtk::Align::Center);

        let label = gtk::Label::new(Some(&tr(body)));
        label.add_css_class("muted");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        wrapper.append(&label);
        wrapper.upcast()
    }
}
